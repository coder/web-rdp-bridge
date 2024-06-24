// @ts-check
/**
 * @todo Still need to polish up and test the logic for setting up the form
 * detector
 *
 * @file Defines the custom logic for patching in UI changes/behavior into the
 * base Devolutions Gateway Angular app.
 *
 * Defined as a JS file to remove the need to have a separate compilation step.
 * It is highly recommended that you work on this file from within VS Code so
 * that you can take advantage of the @ts-check directive and get some type-
 * checking still.
 *
 * A lot of the HTML selectors in this file will look nonstandard. This is
 * because they are actually custom Angular components.
 *
 * @typedef {Readonly<{ querySelector: string; value: string; }>} FormFieldEntry
 * @typedef {Readonly<Record<string, FormFieldEntry>>} FormFieldEntries
 */

/**
 * The communication protocol to set Devolutions to. Right now, this does not
 * need to be dynamic, and can be hard-coded as a constant.
 */
const PROTOCOL = "RDP";

/**
 * The hostname to use with Devolutions. Can be a hard-coded constant for right
 * now; may need to change in the future?
 */
const HOSTNAME = "localhost";

/**
 * How often to poll the screen for the main Devolutions form.
 */
const SCREEN_POLL_INTERVAL_MS = 500;

/**
 * The fields in the Devolutions sign-in form that should be populated with
 * values from the Coder workspace.
 *
 * All properties should be defined as placeholder templates in the form
 * {{ VALUE_NAME }}. The Coder module, when spun up, should then run some logic
 * to replace the template slots with actual values. These values should never
 * change from within JavaScript itself.
 *
 * @satisfies {FormFieldEntries}
 */
const coderFormFieldEntries = {
  /** @readonly */
  username: {
    querySelector: "web-client-username-control input",
    value: "Administrator",
  },

  /** @readonly */
  password: {
    querySelector: "web-client-password-control input",
    value: "coderRDP!",
  },
};

/**
 * Handles typing in the values for the input form, dispatching each character
 * as an event. This function assumes that all characters in the input will be
 * UTF-8.
 *
 * @todo See if there's a better way to implement this logic. One problem is
 * that because it sets the .value property on an element before dispatching an
 * event, Angular is going to complain. As far as I can tell, this won't ever
 * break anything, but it's a warning, because Angular patches a lot of browser
 * APIs to support change detection.
 *
 * @param {HTMLInputElement} inputField
 * @param {string} inputText
 * @returns {Promise<void[]>}
 */
function setInputValue(inputField, inputText) {
  const characterDelayMs = 5;
  const continueEventName = "continue";

  const promise = new Promise((resolve, reject) => {
    // -1 indicates a "pre-write" for clearing out the input before trying to
    // write new text to it
    let i = -1;

    /** @return {void} */
    const handleNextCharIndex = () => {
      if (i === inputText.length) {
        resolve(void 0);
        return;
      }

      const currentChar = inputText[i];
      if (i !== -1 && currentChar === undefined) {
        throw new Error("Went out of bounds");
      }

      try {
        inputField.addEventListener(
          continueEventName,
          () => {
            i++;
            const newDelay = (i + 1) * characterDelayMs;
            window.setTimeout(handleNextCharIndex, newDelay);
          },
          { once: true }
        );

        const continueEvent = new CustomEvent(continueEventName);
        const inputEvent = new Event("input", {
          bubbles: true,
          cancelable: true,
        });

        if (i === -1) {
          inputField.value = "";
        } else {
          inputField.value = `${inputField.value}${currentChar}`;
        }

        inputField.dispatchEvent(inputEvent);
        inputField.dispatchEvent(continueEvent);
      } catch (err) {
        reject(err);
      }
    };

    window.setTimeout(handleNextCharIndex, characterDelayMs);
  });

  return promise;
}

/**
 * Takes a Devolutions remote session form, auto-fills it with data, and then
 * submits it.
 *
 * The logic here is more convoluted than it should be for two main reasons:
 * 1. Devolutions' HTML markup has errors. There are labels, but they aren't
 *    bound to the inputs they're supposed to describe. This means no easy hooks
 *    for selecting the elements, unfortunately.
 * 2. Trying to modify the .value properties on some of the inputs doesn't
 *    work. Probably some combo of Angular data-binding and some inputs having
 *    the readonly attribute. Have to simulate user input to get around this.
 *
 * @param {HTMLFormElement} myForm
 * @returns {Promise<void>}
 */
async function autoSubmitForm(myForm) {
  const setProtocolValue = () => {
    /** @type {HTMLDivElement | null} */
    const protocolDropdownTrigger = myForm.querySelector(`div[role="button"]`);
    if (protocolDropdownTrigger === null) {
      throw new Error("No clickable trigger for setting protocol value");
    }

    protocolDropdownTrigger.click();

    // Can't use form as container for querying the list of dropdown options,
    // because the elements don't actually exist inside the form. I *think* this
    // is to avoid CSS stacking context issues with the dropdown
    /** @type {HTMLLIElement | null} */
    const protocolOption = document.querySelector(
      `p-dropdownitem[ng-reflect-label="${PROTOCOL}"] li`
    );

    if (protocolOption === null) {
      throw new Error(
        "Unable to find protocol option on screen that matches desired protocol"
      );
    }

    protocolOption.click();
  };

  const setHostname = () => {
    /** @type {HTMLInputElement | null} */
    const hostnameInput = myForm.querySelector("p-autocomplete#hostname input");

    if (hostnameInput === null) {
      throw new Error("Unable to find field for adding hostname");
    }

    return setInputValue(hostnameInput, HOSTNAME);
  };

  const setCoderFormFieldValues = async () => {
    const rdpSubsection = myForm.querySelector("rdp-form");
    if (rdpSubsection === null) {
      throw new Error(
        "Unable to find RDP subsection. Is the value of the protocol set to RDP?"
      );
    }

    const iterable = Object.values(coderFormFieldEntries);
    for (const { value, querySelector } of iterable) {
      /** @type {HTMLInputElement | null} */
      const input = document.querySelector(querySelector);

      if (input === null) {
        throw new Error(
          `Unable to element that matches query "${querySelector}`
        );
      }

      await setInputValue(input, value);
    }
  };

  const triggerSubmission = () => {
    /** @type {HTMLButtonElement | null} */
    const submitButton = myForm.querySelector(
      'p-button[ng-reflect-type="submit"] button'
    );

    if (submitButton === null) {
      throw new Error("Unable to find submission button");
    }

    if (submitButton.disabled) {
      throw new Error(
        "Unable to submit form because submit button is disabled. Are all fields filled out correctly?"
      );
    }

    submitButton.click();
  };

  setProtocolValue();
  await setHostname();
  await setCoderFormFieldValues();
  triggerSubmission();
}

/**
 * Sets up logic for auto-populating the form data when the form appears on
 * screen.
 *
 * @returns {void}
 */
function setupFormDetection() {
  /** @type {HTMLFormElement | null} */
  let formValueFromLastMutation = null;

  /** @returns {void} */
  const onDynamicTabMutation = () => {
    /** @type {HTMLFormElement | null} */
    const latestForm = document.querySelector("web-client-form > form");

    if (latestForm === null) {
      formValueFromLastMutation = null;
      return;
    }

    // Only try to auto-fill it if we went from having no form on screen to
    // having a form on screen. That way, we don't accidentally override the
    // form if the user is trying to customize values, and this essentially
    // make the form values function as default values.
    if (formValueFromLastMutation === null) {
      autoSubmitForm(latestForm);
    }

    formValueFromLastMutation = latestForm;
  };

  /** @type {number | undefined} */
  let intervalId = undefined;

  /** @returns {void} */
  const checkScreenForDynamicTab = () => {
    const dynamicTab = document.querySelector("web-client-dynamic-tab");
    if (dynamicTab === null) {
      return;
    }

    window.clearInterval(intervalId);

    // Call the mutation callback manually, to ensure it runs at least once
    onDynamicTabMutation();
    const dynamicTabObserver = new MutationObserver(onDynamicTabMutation);
    dynamicTabObserver.observe(dynamicTab, {
      childList: true,
    });
  };

  intervalId = window.setTimeout(
    checkScreenForDynamicTab,
    SCREEN_POLL_INTERVAL_MS
  );
}

/**
 * Sets up custom styles for hiding default Devolutions elements that Coder
 * users shouldn't need to care about.
 *
 * @returns {void}
 */
function setupObscuringStyles() {
  const styleContainer = document.createElement("style");
  styleContainer.type = "text/css";

  styleContainer.innerHTML = `
    /* app-menu corresponds to the sidebar of the default view. */
    app-menu {
      display: none !important;
    }
  `;

  document.head.appendChild(styleContainer);
}

// setupObscuringStyles();
// window.addEventListener("DOMContentLoaded", setupFormDetection);
